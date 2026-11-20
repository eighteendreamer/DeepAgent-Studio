use super::*;

use deepagent_app_core::settings::ResponsesApiSettings;
use deepagent_models::{ModelClient, ModelConfig, ResponseRequest, ToolSchema};

#[derive(Default)]
struct CountingTransport {
    requests: Mutex<Vec<serde_json::Value>>,
}

#[async_trait]
impl HttpTransport for CountingTransport {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let sink: &mut dyn deepagent_models::transport::EventSink =
            &mut crate::sequence_fixture::sink(sink);
        self.requests
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body)?);
        sink.on_event(r#"{"type":"response.completed","response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash","output":[],"status":"completed"}}"#)?;
        Ok(())
    }
}

#[tokio::test]
async fn unsupported_responses_named_tool_choices_are_rejected_before_transport() {
    let cases = [
        (
            r#"{"tool_choice":{"type":"custom","name":"apply_patch"}}"#,
            ToolSchema::function(
                "apply_patch",
                "Apply a patch.",
                serde_json::json!({"type":"object","properties":{}}),
            ),
        ),
        (
            r#"{"tool_choice":{"type":"web_search"}}"#,
            ToolSchema::function(
                "web_search",
                "Search the web.",
                serde_json::json!({"type":"object","properties":{}}),
            ),
        ),
    ];

    let mut violations = Vec::new();
    for (raw, tool) in cases {
        let stored: serde_json::Value = serde_json::from_str(raw).unwrap();
        let (accepted, requests) =
            observe_choice(stored["tool_choice"].clone(), vec![tool], true, false).await;
        if accepted || !requests.is_empty() {
            violations.push((raw.to_string(), requests.first().cloned()));
        }
    }

    assert!(
        violations.is_empty(),
        "unsupported named tool_choice reached provider transport: {violations:?}"
    );
}

fn function_tool(name: &str) -> ToolSchema {
    ToolSchema::function(
        name,
        "Audit function.",
        serde_json::json!({"type":"object","properties":{}}),
    )
}

async fn observe_choice(
    choice: serde_json::Value,
    tools: Vec<ToolSchema>,
    from_defaults: bool,
    cancelled_entrypoint: bool,
) -> (bool, Vec<serde_json::Value>) {
    let transport = Arc::new(CountingTransport::default());
    let mut config = ModelConfig::deepseek("audit-placeholder");
    let mut request = ResponseRequest::new(
        "deepseek-v4-flash",
        vec![deepagent_core::message::Message::user(
            "Audit choice linkage",
        )],
    )
    .with_tools(tools);
    if from_defaults {
        config.defaults.tool_choice = Some(choice);
    } else {
        request.tool_choice = Some(choice);
    }
    let client = ModelClient::new(transport.clone(), config);
    let result = if cancelled_entrypoint {
        client
            .stream_response_cancelled(request, Arc::new(AtomicBool::new(false)))
            .await
    } else {
        client.stream_response(request).await
    };
    let requests = transport.requests.lock().unwrap().clone();
    (result.is_ok(), requests)
}

#[tokio::test]
async fn responses_choice_linkage_normal_controls_reach_transport() {
    for from_defaults in [false, true] {
        for cancelled_entrypoint in [false, true] {
            for choice in [
                serde_json::json!("auto"),
                serde_json::json!("none"),
                serde_json::json!("required"),
                serde_json::json!({"type":"function","name":"read_file"}),
            ] {
                let (accepted, wire) = observe_choice(
                    choice.clone(),
                    vec![function_tool("read_file")],
                    from_defaults,
                    cancelled_entrypoint,
                )
                .await;
                assert!(accepted);
                assert_eq!(wire.len(), 1);
                assert_eq!(wire[0]["tool_choice"], choice);
                assert_eq!(wire[0]["tools"][0]["type"], "function");
                assert_eq!(wire[0]["tools"][0]["name"], "read_file");
            }
        }
    }
}

#[tokio::test]
async fn audit_responses_named_choice_requires_final_function_tool() {
    let mut violations = Vec::new();
    for from_defaults in [false, true] {
        for cancelled_entrypoint in [false, true] {
            for (label, name, tools, expected_valid) in [
                ("no-tools", "missing_tool", Vec::new(), false),
                (
                    "different-function",
                    "missing_tool",
                    vec![function_tool("read_file")],
                    false,
                ),
                (
                    "explicit-function-same-name",
                    "apply_patch",
                    vec![function_tool("apply_patch")],
                    true,
                ),
            ] {
                let (accepted, wire) = observe_choice(
                    serde_json::json!({"type":"function","name":name}),
                    tools,
                    from_defaults,
                    cancelled_entrypoint,
                )
                .await;
                if expected_valid {
                    assert!(accepted, "an explicit function tool must remain callable");
                    assert!(wire
                        .iter()
                        .all(
                            |request| request["tools"].as_array().is_some_and(|tools| tools
                                .iter()
                                .any(|tool| tool["type"] == "function" && tool["name"] == name))
                        ));
                } else {
                    for request in &wire {
                        assert!(
                            !request["tools"].as_array().is_some_and(|tools| tools
                                .iter()
                                .any(|tool| tool["type"] == "function" && tool["name"] == name)),
                            "fixture must prove that no matching function is present"
                        );
                    }
                }
                eprintln!(
                    "H-158 linkage: case={label} defaults={from_defaults} cancel_entry={cancelled_entrypoint} accepted={accepted} wire={wire:?}"
                );
                if expected_valid {
                    if !accepted || wire.is_empty() {
                        violations.push((label, from_defaults, cancelled_entrypoint));
                    }
                } else if accepted || !wire.is_empty() {
                    violations.push((label, from_defaults, cancelled_entrypoint));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "unresolved function choice reached wire: {violations:?}"
    );
}

async fn observe_saved_chat_choice(choice: serde_json::Value) -> (bool, Vec<serde_json::Value>) {
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
    db.seed_responses_developer_json(&serde_json::json!({"tool_choice":choice}).to_string())
        .unwrap();
    let transport = Arc::new(CountingTransport::default());
    let chat = ChatService::new(db, settings, transport.clone(), root.path());
    let result = chat
        .run_in_session(
            "Audit missing function choice.",
            None,
            None,
            None,
            Vec::new(),
            None,
            false,
            None,
            |_| {},
            |_| {},
        )
        .await;
    let wire = transport.requests.lock().unwrap().clone();
    (result.is_ok(), wire)
}

#[tokio::test]
async fn saved_chat_choice_normal_controls_reach_transport() {
    for choice in [
        serde_json::json!("auto"),
        serde_json::json!("none"),
        serde_json::json!("required"),
        serde_json::json!({"type":"function","name":"read_file"}),
    ] {
        let (accepted, wire) = observe_saved_chat_choice(choice.clone()).await;
        assert!(accepted);
        assert_eq!(wire.len(), 1);
        assert_eq!(wire[0]["tool_choice"], choice);
        assert!(wire[0]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["type"] == "function" && tool["name"] == "read_file"));
    }
}

#[tokio::test]
async fn audit_saved_named_choice_without_matching_chat_tool_is_rejected() {
    let (accepted, wire) = observe_saved_chat_choice(
        serde_json::json!({"type":"function","name":"audit_missing_function_158"}),
    )
    .await;
    for request in wire.iter() {
        assert_eq!(request["tool_choice"]["name"], "audit_missing_function_158");
        assert!(request["tools"]
            .as_array()
            .is_some_and(|tools| !tools.is_empty()
                && tools
                    .iter()
                    .all(|tool| tool["name"] != "audit_missing_function_158")));
    }
    eprintln!(
        "H-158 saved choice: accepted={} transport_calls={}",
        accepted,
        wire.len()
    );
    assert!(
        !accepted && wire.is_empty(),
        "persisted unresolved choice reached formal ChatService transport"
    );
}

#[tokio::test]
async fn persisted_responses_tool_choice_survives_restart() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("settings.sqlite");
    let secrets = Arc::new(MemorySecretStore::new());
    let discovery = Arc::new(MockTransport::with_get_json(
        r#"{"object":"list","data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
    ));
    let valid = serde_json::json!({"type":"function","name":"read_file"});
    for developer in [false, true] {
        {
            let db = Arc::new(Database::open(&path).unwrap());
            let settings = SettingsService::new(db.clone(), discovery.clone(), secrets.clone());
            crate::registry_fixture::configure(&settings, "synthetic-key")
                .await
                .unwrap();
            db.seed_responses_settings(ResponsesApiSettings::default())
                .unwrap();
            if developer {
                db.seed_responses_developer_json(
                    &serde_json::json!({"tool_choice":valid}).to_string(),
                )
                .unwrap();
            } else {
                db.seed_responses_settings(ResponsesApiSettings {
                    tool_choice: Some(valid.clone()),
                    ..ResponsesApiSettings::default()
                })
                .unwrap();
            }
        }
        let db = Arc::new(Database::open(&path).unwrap());
        let settings = SettingsService::new(db.clone(), discovery.clone(), secrets.clone());
        assert_eq!(
            settings
                .load()
                .unwrap()
                .unwrap()
                .responses
                .effective_tool_choice(),
            Some(valid.clone())
        );
    }
}
