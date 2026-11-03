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
        self.requests
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body)?);
        sink.on_event(r#"{"type":"response.completed","response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash","output":[],"status":"completed"}}"#)?;
        Ok(())
    }
}

#[tokio::test]
#[ignore = "audit reproducer: DeepSeek Responses named tool_choice only supports function"]
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
        let parsed = ResponsesApiSettings::from_developer_json(raw);
        let accepted = parsed.is_ok();
        let transport = Arc::new(CountingTransport::default());

        if let Ok(settings) = parsed {
            let mut config = ModelConfig::deepseek("audit-placeholder");
            config.defaults.native_web_search = tool.function.name == "web_search";
            let client = ModelClient::new(transport.clone(), config);
            let request = ResponseRequest::new(
                "deepseek-v4-pro",
                vec![deepagent_core::message::Message::user("Audit tool choice")],
            )
            .with_tools(vec![tool])
            .with_tool_choice(settings.effective_tool_choice().unwrap());
            client.stream_response(request).await.unwrap();
        }

        let requests = transport.requests.lock().unwrap();
        eprintln!(
            "Responses tool_choice audit: accepted={accepted} wire={:?}",
            requests.first()
        );
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
#[ignore = "audit H-158 extension: named function must exist in the final function tool snapshot"]
async fn audit_responses_named_choice_requires_final_function_tool() {
    let mut violations = Vec::new();
    for from_defaults in [false, true] {
        for cancelled_entrypoint in [false, true] {
            for (label, name, tools) in [
                ("no-tools", "missing_tool", Vec::new()),
                (
                    "different-function",
                    "missing_tool",
                    vec![function_tool("read_file")],
                ),
                (
                    "converted-custom",
                    "apply_patch",
                    vec![function_tool("apply_patch")],
                ),
            ] {
                let (accepted, wire) = observe_choice(
                    serde_json::json!({"type":"function","name":name}),
                    tools,
                    from_defaults,
                    cancelled_entrypoint,
                )
                .await;
                for request in &wire {
                    assert!(
                        !request["tools"].as_array().is_some_and(|tools| tools
                            .iter()
                            .any(|tool| tool["type"] == "function" && tool["name"] == name)),
                        "fixture must prove that no matching function is present"
                    );
                }
                eprintln!(
                    "H-158 linkage: case={label} defaults={from_defaults} cancel_entry={cancelled_entrypoint} accepted={accepted} wire={wire:?}"
                );
                if accepted || !wire.is_empty() {
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
    settings.initialize("sk-audit-placeholder").await.unwrap();
    settings
        .set_responses_developer_json(&serde_json::json!({"tool_choice":choice}).to_string())
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
#[ignore = "audit H-158 extension: persisted named choice is not checked against ChatService tools"]
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
